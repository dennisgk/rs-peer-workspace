use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use rs_peer_workspace_shared::relay::{AuthRole, PeerToProxy, ProxyToPeer, TurnCredentials};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about)]
struct Args {
    #[arg(long, default_value = "0.0.0.0:9000")]
    bind: String,
    #[arg(long)]
    proxy_password: String,
    #[arg(long)]
    turn_url: Option<String>,
    #[arg(long, default_value = "3478")]
    turn_port: u16,
    #[arg(long, default_value = "https://api.ipify.org")]
    public_ip_service: String,
    #[arg(long, default_value = "peer")]
    turn_username: String,
    #[arg(long, default_value = "peer-secret")]
    turn_password: String,
}

#[derive(Debug, Clone)]
struct ServerRegistration {
    conn_id: Uuid,
    server_password: String,
}

#[derive(Debug, Clone)]
struct Session {
    session_id: Uuid,
    server_conn_id: Uuid,
    client_conn_id: Uuid,
}

#[derive(Debug)]
struct ProxyState {
    connections: HashMap<Uuid, mpsc::UnboundedSender<Message>>,
    conn_roles: HashMap<Uuid, AuthRole>,
    servers: HashMap<String, ServerRegistration>,
    sessions: HashMap<Uuid, Session>,
}

impl ProxyState {
    fn new() -> Self {
        Self {
            connections: HashMap::new(),
            conn_roles: HashMap::new(),
            servers: HashMap::new(),
            sessions: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct AppState {
    proxy_password: String,
    turn: Option<TurnCredentials>,
    state: Arc<Mutex<ProxyState>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let addr: SocketAddr = args.bind.parse()?;
    let advertised_turn_url = resolve_turn_url(&args).await;

    let app_state = AppState {
        proxy_password: args.proxy_password,
        turn: advertised_turn_url.map(|url| TurnCredentials {
            url,
            username: args.turn_username,
            password: args.turn_password,
        }),
        state: Arc::new(Mutex::new(ProxyState::new())),
    };

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(app_state.clone());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("proxy listening on {}", addr);
    if let Some(turn) = &app_state.turn {
        println!("advertising TURN endpoint {}", turn.url);
    } else {
        println!("TURN unavailable; P2P disabled and sessions will use WebSocket relay");
    }
    axum::serve(listener, app).await?;
    Ok(())
}

async fn resolve_turn_url(args: &Args) -> Option<String> {
    if let Some(explicit) = &args.turn_url {
        return Some(explicit.clone());
    }

    let explicit_ip = std::env::var("TURN_PUBLIC_IP")
        .ok()
        .or_else(|| std::env::var("PUBLIC_IP").ok())
        .filter(|v| !v.trim().is_empty());
    if let Some(ip) = explicit_ip {
        return Some(format!("turn:{}:{}", ip.trim(), args.turn_port));
    }

    match reqwest::get(&args.public_ip_service).await {
        Ok(resp) => match resp.text().await {
            Ok(ip) if !ip.trim().is_empty() => Some(format!("turn:{}:{}", ip.trim(), args.turn_port)),
            _ => {
                eprintln!("failed to parse public IP response; disabling TURN and using WebSocket relay");
                None
            }
        },
        Err(err) => {
            eprintln!(
                "failed to detect public IP from {} ({err}); disabling TURN and using WebSocket relay",
                args.public_ip_service
            );
            None
        }
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(app): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, app))
}

async fn handle_socket(socket: WebSocket, app: AppState) {
    let conn_id = Uuid::new_v4();
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<Message>();

    {
        let mut state = app.state.lock().await;
        state.connections.insert(conn_id, outgoing_tx);
    }

    let writer = tokio::spawn(async move {
        while let Some(msg) = outgoing_rx.recv().await {
            if ws_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut role: Option<AuthRole> = None;
    let mut server_name: Option<String> = None;

    while let Some(msg_result) = ws_rx.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(_) => break,
        };

        let Message::Text(text) = msg else {
            continue;
        };

        let Ok(peer_msg) = serde_json::from_str::<PeerToProxy>(&text) else {
            continue;
        };

        if role.is_none() {
            let PeerToProxy::AuthProxy { proxy_password, role: parsed_role } = peer_msg else {
                let _ = send_to_connection(&app.state, conn_id, &ProxyToPeer::AuthError {
                    reason: "first message must be auth_proxy".to_string(),
                }).await;
                break;
            };

            if proxy_password != app.proxy_password {
                let _ = send_to_connection(&app.state, conn_id, &ProxyToPeer::AuthError {
                    reason: "invalid proxy password".to_string(),
                }).await;
                break;
            }

            app.state.lock().await.conn_roles.insert(conn_id, parsed_role.clone());
            let _ = send_to_connection(&app.state, conn_id, &ProxyToPeer::AuthOk { role: parsed_role.clone() }).await;
            role = Some(parsed_role);
            continue;
        }

        match role {
            Some(AuthRole::Server) => {
                if server_name.is_none() {
                    let PeerToProxy::RegisterServer { server_name: name, server_password } = peer_msg else {
                        let _ = send_to_connection(&app.state, conn_id, &ProxyToPeer::ConnectionError {
                            reason: "server must register before other actions".to_string(),
                        }).await;
                        break;
                    };

                    let inserted = {
                        let mut state = app.state.lock().await;
                        if state.servers.contains_key(&name) {
                            false
                        } else {
                            state.servers.insert(name.clone(), ServerRegistration { conn_id, server_password });
                            true
                        }
                    };

                    if inserted {
                        let _ = send_to_connection(&app.state, conn_id, &ProxyToPeer::Registered { server_name: name.clone() }).await;
                        server_name = Some(name);
                    } else {
                        let _ = send_to_connection(&app.state, conn_id, &ProxyToPeer::ConnectionError {
                            reason: "server name already registered".to_string(),
                        }).await;
                        break;
                    }
                    continue;
                }

                match peer_msg {
                    PeerToProxy::DisconnectSession { session_id } => {
                        let target_client = {
                            let mut state = app.state.lock().await;
                            state.sessions.remove(&session_id).map(|session| session.client_conn_id)
                        };
                        if let Some(client_conn_id) = target_client {
                            let _ = send_to_connection(&app.state, client_conn_id, &ProxyToPeer::SessionClosed {
                                session_id,
                                reason: "server closed session".to_string(),
                            }).await;
                        }
                    }
                    PeerToProxy::Signal { session_id, signal } => {
                        if let Some(client_conn_id) = {
                            let state = app.state.lock().await;
                            state.sessions.get(&session_id).map(|s| s.client_conn_id)
                        } {
                            let _ = send_to_connection(&app.state, client_conn_id, &ProxyToPeer::PeerSignal {
                                session_id,
                                from: AuthRole::Server,
                                signal,
                            }).await;
                        }
                    }
                    PeerToProxy::RelayData { session_id, payload } => {
                        if let Some(client_conn_id) = {
                            let state = app.state.lock().await;
                            state.sessions.get(&session_id).map(|s| s.client_conn_id)
                        } {
                            let _ = send_to_connection(&app.state, client_conn_id, &ProxyToPeer::RelayData {
                                session_id,
                                payload,
                            }).await;
                        }
                    }
                    _ => {}
                }
            }
            Some(AuthRole::Client) => {
                match peer_msg {
                    PeerToProxy::ConnectServer { server_name, server_password, use_p2p } => {
                        let setup = {
                            let mut state = app.state.lock().await;
                            if let Some(server) = state.servers.get(&server_name).cloned() {
                                if server.server_password != server_password {
                                    Some(Err("invalid server password".to_string()))
                                } else {
                                    let session_id = Uuid::new_v4();
                                    state.sessions.insert(session_id, Session {
                                        session_id,
                                        server_conn_id: server.conn_id,
                                        client_conn_id: conn_id,
                                    });
                                    Some(Ok((session_id, server.conn_id)))
                                }
                            } else {
                                Some(Err("unknown server name".to_string()))
                            }
                        };

                        match setup {
                            Some(Ok((session_id, server_conn_id))) => {
                                let p2p_enabled = use_p2p && app.turn.is_some();
                                let turn_creds = if p2p_enabled { app.turn.clone() } else { None };
                                let _ = send_to_connection(&app.state, conn_id, &ProxyToPeer::Connected {
                                    session_id,
                                    server_name: server_name.clone(),
                                    via_p2p: p2p_enabled,
                                    turn: turn_creds.clone(),
                                }).await;
                                let _ = send_to_connection(&app.state, server_conn_id, &ProxyToPeer::PeerJoined {
                                    session_id,
                                    peer_id: conn_id,
                                    via_p2p: p2p_enabled,
                                    turn: turn_creds,
                                }).await;
                            }
                            Some(Err(reason)) => {
                                let _ = send_to_connection(&app.state, conn_id, &ProxyToPeer::ConnectionError { reason }).await;
                            }
                            None => {}
                        }
                    }
                    PeerToProxy::DisconnectSession { session_id } => {
                        let target_server = {
                            let mut state = app.state.lock().await;
                            state.sessions.remove(&session_id).and_then(|session| {
                                if session.client_conn_id == conn_id { Some(session.server_conn_id) } else { None }
                            })
                        };
                        if let Some(server_conn_id) = target_server {
                            let _ = send_to_connection(&app.state, server_conn_id, &ProxyToPeer::SessionClosed {
                                session_id,
                                reason: "client closed session".to_string(),
                            }).await;
                        }
                    }
                    PeerToProxy::Signal { session_id, signal } => {
                        if let Some(server_conn_id) = {
                            let state = app.state.lock().await;
                            state.sessions.get(&session_id).and_then(|session| {
                                if session.client_conn_id == conn_id { Some(session.server_conn_id) } else { None }
                            })
                        } {
                            let _ = send_to_connection(&app.state, server_conn_id, &ProxyToPeer::PeerSignal {
                                session_id,
                                from: AuthRole::Client,
                                signal,
                            }).await;
                        }
                    }
                    PeerToProxy::RelayData { session_id, payload } => {
                        if let Some(server_conn_id) = {
                            let state = app.state.lock().await;
                            state.sessions.get(&session_id).and_then(|session| {
                                if session.client_conn_id == conn_id { Some(session.server_conn_id) } else { None }
                            })
                        } {
                            let _ = send_to_connection(&app.state, server_conn_id, &ProxyToPeer::RelayData {
                                session_id,
                                payload,
                            }).await;
                        }
                    }
                    _ => {}
                }
            }
            None => break,
        }
    }

    cleanup_connection(&app.state, conn_id, server_name).await;
    writer.abort();
}

async fn send_to_connection(
    state: &Arc<Mutex<ProxyState>>,
    conn_id: Uuid,
    message: &ProxyToPeer,
) -> anyhow::Result<()> {
    let payload = serde_json::to_string(message)?;
    let sender = {
        let state = state.lock().await;
        state.connections.get(&conn_id).cloned()
    };
    if let Some(tx) = sender {
        let _ = tx.send(Message::Text(payload.into()));
    }
    Ok(())
}

async fn cleanup_connection(state: &Arc<Mutex<ProxyState>>, conn_id: Uuid, server_name: Option<String>) {
    let mut notifications: Vec<(Uuid, ProxyToPeer)> = Vec::new();
    {
        let mut locked = state.lock().await;
        locked.connections.remove(&conn_id);
        locked.conn_roles.remove(&conn_id);
        if let Some(name) = server_name {
            locked.servers.remove(&name);
        }

        let affected_sessions: Vec<Uuid> = locked.sessions.values()
            .filter(|session| session.server_conn_id == conn_id || session.client_conn_id == conn_id)
            .map(|session| session.session_id)
            .collect();

        for session_id in affected_sessions {
            if let Some(session) = locked.sessions.remove(&session_id) {
                if session.server_conn_id == conn_id {
                    notifications.push((session.client_conn_id, ProxyToPeer::SessionClosed {
                        session_id,
                        reason: "server disconnected".to_string(),
                    }));
                } else {
                    notifications.push((session.server_conn_id, ProxyToPeer::SessionClosed {
                        session_id,
                        reason: "client disconnected".to_string(),
                    }));
                }
            }
        }
    }

    for (target, message) in notifications {
        let _ = send_to_connection(state, target, &message).await;
    }
}
