use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about)]
struct Args {
    #[arg(long, default_value = "0.0.0.0:9000")]
    bind: String,
    #[arg(long)]
    proxy_password: String,
    #[arg(long, default_value = "turn:coturn:3478")]
    turn_url: String,
    #[arg(long, default_value = "peer")]
    turn_username: String,
    #[arg(long, default_value = "peer-secret")]
    turn_password: String,
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
    ListServers,
    ConnectServer {
        server_name: String,
        server_password: String,
        use_p2p: bool,
    },
    ClientCommand {
        session_id: Uuid,
        command: String,
    },
    DisconnectSession {
        session_id: Uuid,
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
    ServersList {
        servers: Vec<String>,
    },
    Connected {
        session_id: Uuid,
        server_name: String,
        via_p2p: bool,
        turn: Option<TurnCredentials>,
    },
    ConnectionError {
        reason: String,
    },
    ClientConnected {
        session_id: Uuid,
        client_id: Uuid,
    },
    RunCommand {
        session_id: Uuid,
        command: String,
    },
    Output {
        session_id: Uuid,
        output: String,
        done: bool,
    },
    SessionClosed {
        session_id: Uuid,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnCredentials {
    url: String,
    username: String,
    password: String,
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
    turn: TurnCredentials,
    state: Arc<Mutex<ProxyState>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let addr: SocketAddr = args.bind.parse()?;

    let app_state = AppState {
        proxy_password: args.proxy_password,
        turn: TurnCredentials {
            url: args.turn_url,
            username: args.turn_username,
            password: args.turn_password,
        },
        state: Arc::new(Mutex::new(ProxyState::new())),
    };

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(app_state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("proxy listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
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

        if role.is_none() {
            let parsed = serde_json::from_str::<ClientToProxy>(&text);
            let Ok(ClientToProxy::AuthProxy {
                proxy_password,
                role: parsed_role,
            }) = parsed
            else {
                let _ = send_to_connection(
                    &app.state,
                    conn_id,
                    &ProxyToPeer::AuthError {
                        reason: "first message must be auth_proxy".to_string(),
                    },
                )
                .await;
                break;
            };

            if proxy_password != app.proxy_password {
                let _ = send_to_connection(
                    &app.state,
                    conn_id,
                    &ProxyToPeer::AuthError {
                        reason: "invalid proxy password".to_string(),
                    },
                )
                .await;
                break;
            }

            {
                let mut state = app.state.lock().await;
                state.conn_roles.insert(conn_id, parsed_role.clone());
            }

            let _ = send_to_connection(
                &app.state,
                conn_id,
                &ProxyToPeer::AuthOk {
                    role: parsed_role.clone(),
                },
            )
            .await;
            role = Some(parsed_role);
            continue;
        }

        match role {
            Some(AuthRole::Server) => {
                if server_name.is_none() {
                    let parsed = serde_json::from_str::<ClientToProxy>(&text);
                    let Ok(ClientToProxy::RegisterServer {
                        server_name: name,
                        server_password,
                    }) = parsed
                    else {
                        let _ = send_to_connection(
                            &app.state,
                            conn_id,
                            &ProxyToPeer::ConnectionError {
                                reason: "server must register before other actions".to_string(),
                            },
                        )
                        .await;
                        break;
                    };

                    let mut ok = true;
                    {
                        let mut state = app.state.lock().await;
                        if state.servers.contains_key(&name) {
                            ok = false;
                        } else {
                            state.servers.insert(
                                name.clone(),
                                ServerRegistration {
                                    conn_id,
                                    server_password,
                                },
                            );
                        }
                    }

                    if ok {
                        let _ = send_to_connection(
                            &app.state,
                            conn_id,
                            &ProxyToPeer::Registered {
                                server_name: name.clone(),
                            },
                        )
                        .await;
                        server_name = Some(name);
                    } else {
                        let _ = send_to_connection(
                            &app.state,
                            conn_id,
                            &ProxyToPeer::ConnectionError {
                                reason: "server name already registered".to_string(),
                            },
                        )
                        .await;
                        break;
                    }

                    continue;
                }

                let Ok(server_msg) = serde_json::from_str::<ServerToProxy>(&text) else {
                    continue;
                };

                match server_msg {
                    ServerToProxy::CommandOutput {
                        session_id,
                        output,
                        done,
                    } => {
                        let target_client = {
                            let state = app.state.lock().await;
                            state.sessions.get(&session_id).map(|s| s.client_conn_id)
                        };

                        if let Some(client_conn_id) = target_client {
                            let _ = send_to_connection(
                                &app.state,
                                client_conn_id,
                                &ProxyToPeer::Output {
                                    session_id,
                                    output,
                                    done,
                                },
                            )
                            .await;
                        }
                    }
                    ServerToProxy::ServerDisconnectSession { session_id } => {
                        let target_client = {
                            let mut state = app.state.lock().await;
                            state
                                .sessions
                                .remove(&session_id)
                                .map(|session| session.client_conn_id)
                        };

                        if let Some(client_conn_id) = target_client {
                            let _ = send_to_connection(
                                &app.state,
                                client_conn_id,
                                &ProxyToPeer::SessionClosed {
                                    session_id,
                                    reason: "server closed session".to_string(),
                                },
                            )
                            .await;
                        }
                    }
                }
            }
            Some(AuthRole::Client) => {
                let Ok(client_msg) = serde_json::from_str::<ClientToProxy>(&text) else {
                    continue;
                };

                match client_msg {
                    ClientToProxy::ListServers => {
                        let servers = {
                            let state = app.state.lock().await;
                            state.servers.keys().cloned().collect::<Vec<_>>()
                        };
                        let _ = send_to_connection(
                            &app.state,
                            conn_id,
                            &ProxyToPeer::ServersList { servers },
                        )
                        .await;
                    }
                    ClientToProxy::ConnectServer {
                        server_name,
                        server_password,
                        use_p2p,
                    } => {
                        let setup = {
                            let mut state = app.state.lock().await;
                            if let Some(server) = state.servers.get(&server_name).cloned() {
                                if server.server_password != server_password {
                                    Some(Err("invalid server password".to_string()))
                                } else {
                                    let session_id = Uuid::new_v4();
                                    state.sessions.insert(
                                        session_id,
                                        Session {
                                            session_id,
                                            server_conn_id: server.conn_id,
                                            client_conn_id: conn_id,
                                        },
                                    );
                                    Some(Ok((session_id, server.conn_id)))
                                }
                            } else {
                                Some(Err("unknown server name".to_string()))
                            }
                        };

                        match setup {
                            Some(Ok((session_id, server_conn_id))) => {
                                let _ = send_to_connection(
                                    &app.state,
                                    conn_id,
                                    &ProxyToPeer::Connected {
                                        session_id,
                                        server_name: server_name.clone(),
                                        via_p2p: use_p2p,
                                        turn: if use_p2p {
                                            Some(app.turn.clone())
                                        } else {
                                            None
                                        },
                                    },
                                )
                                .await;

                                let _ = send_to_connection(
                                    &app.state,
                                    server_conn_id,
                                    &ProxyToPeer::ClientConnected {
                                        session_id,
                                        client_id: conn_id,
                                    },
                                )
                                .await;
                            }
                            Some(Err(reason)) => {
                                let _ = send_to_connection(
                                    &app.state,
                                    conn_id,
                                    &ProxyToPeer::ConnectionError { reason },
                                )
                                .await;
                            }
                            None => {}
                        }
                    }
                    ClientToProxy::ClientCommand {
                        session_id,
                        command,
                    } => {
                        let target_server = {
                            let state = app.state.lock().await;
                            state.sessions.get(&session_id).and_then(|session| {
                                if session.client_conn_id == conn_id {
                                    Some(session.server_conn_id)
                                } else {
                                    None
                                }
                            })
                        };

                        if let Some(server_conn_id) = target_server {
                            let _ = send_to_connection(
                                &app.state,
                                server_conn_id,
                                &ProxyToPeer::RunCommand {
                                    session_id,
                                    command,
                                },
                            )
                            .await;
                        }
                    }
                    ClientToProxy::DisconnectSession { session_id } => {
                        let target_server = {
                            let mut state = app.state.lock().await;
                            state.sessions.remove(&session_id).and_then(|session| {
                                if session.client_conn_id == conn_id {
                                    Some(session.server_conn_id)
                                } else {
                                    None
                                }
                            })
                        };

                        if let Some(server_conn_id) = target_server {
                            let _ = send_to_connection(
                                &app.state,
                                server_conn_id,
                                &ProxyToPeer::SessionClosed {
                                    session_id,
                                    reason: "client closed session".to_string(),
                                },
                            )
                            .await;
                        }
                    }
                    ClientToProxy::AuthProxy { .. } | ClientToProxy::RegisterServer { .. } => {}
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

async fn cleanup_connection(
    state: &Arc<Mutex<ProxyState>>,
    conn_id: Uuid,
    server_name: Option<String>,
) {
    let mut notifications: Vec<(Uuid, ProxyToPeer)> = Vec::new();

    {
        let mut locked = state.lock().await;
        locked.connections.remove(&conn_id);
        locked.conn_roles.remove(&conn_id);

        if let Some(name) = server_name {
            locked.servers.remove(&name);
        }

        let affected_sessions: Vec<Uuid> = locked
            .sessions
            .values()
            .filter(|session| session.server_conn_id == conn_id || session.client_conn_id == conn_id)
            .map(|session| session.session_id)
            .collect();

        for session_id in affected_sessions {
            if let Some(session) = locked.sessions.remove(&session_id) {
                if session.server_conn_id == conn_id {
                    notifications.push((
                        session.client_conn_id,
                        ProxyToPeer::SessionClosed {
                            session_id,
                            reason: "server disconnected".to_string(),
                        },
                    ));
                } else {
                    notifications.push((
                        session.server_conn_id,
                        ProxyToPeer::SessionClosed {
                            session_id,
                            reason: "client disconnected".to_string(),
                        },
                    ));
                }
            }
        }
    }

    for (target, message) in notifications {
        let _ = send_to_connection(state, target, &message).await;
    }
}
