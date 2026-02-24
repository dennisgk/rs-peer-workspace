use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use eframe::egui;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
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
    ClientSignal {
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
    Output {
        session_id: Uuid,
        output: String,
        done: bool,
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

#[derive(Debug, Clone)]
struct ConnectConfig {
    proxy_addr: String,
    proxy_password: String,
    server_name: String,
    server_password: String,
    use_p2p: bool,
}

#[derive(Debug)]
enum NetCommand {
    SendCommand(String),
    Disconnect,
}

#[derive(Debug)]
enum NetEvent {
    Status(String),
    Transport(String),
    Servers(Vec<String>),
    CommandSent {
        transport: String,
        command: String,
    },
    Connected {
        session_id: Uuid,
        server_name: String,
        via_p2p: bool,
        turn: Option<TurnCredentials>,
    },
    Output(String),
    SessionClosed(String),
    Error(String),
}

struct ClientApp {
    show_connect_dialog: bool,
    show_terminal_window: bool,
    proxy_addr: String,
    proxy_password: String,
    server_name: String,
    server_password: String,
    use_p2p: bool,
    known_servers: Vec<String>,
    selected_server_index: usize,
    logs: String,
    command_input: String,
    status: String,
    transport: String,
    event_rx: Option<Receiver<NetEvent>>,
    command_tx: Option<tokio_mpsc::UnboundedSender<NetCommand>>,
    session_id: Option<Uuid>,
}

impl Default for ClientApp {
    fn default() -> Self {
        Self {
            show_connect_dialog: false,
            show_terminal_window: false,
            proxy_addr: "ws://127.0.0.1:9000/ws".to_string(),
            proxy_password: String::new(),
            server_name: String::new(),
            server_password: String::new(),
            use_p2p: true,
            known_servers: vec!["<manual>".to_string()],
            selected_server_index: 0,
            logs: String::new(),
            command_input: String::new(),
            status: "Disconnected".to_string(),
            transport: "None".to_string(),
            event_rx: None,
            command_tx: None,
            session_id: None,
        }
    }
}

impl eframe::App for ClientApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_events();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Server");
                egui::ComboBox::from_id_salt("server_combo")
                    .selected_text(
                        self.known_servers
                            .get(self.selected_server_index)
                            .cloned()
                            .unwrap_or_else(|| "<manual>".to_string()),
                    )
                    .show_ui(ui, |ui| {
                        for (idx, name) in self.known_servers.iter().enumerate() {
                            if ui
                                .selectable_label(self.selected_server_index == idx, name)
                                .clicked()
                            {
                                self.selected_server_index = idx;
                                if name != "<manual>" {
                                    self.server_name = name.clone();
                                }
                            }
                        }
                    });

                if ui.button("Terminal").clicked() {
                    self.show_connect_dialog = true;
                }

                ui.separator();
                ui.label(format!("Status: {}", self.status));
                ui.separator();
                ui.label(format!("Transport: {}", self.transport));
            });
        });

        if self.show_connect_dialog {
            let mut open = self.show_connect_dialog;
            egui::Window::new("Terminal Connection")
                .open(&mut open)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Proxy Address (ws://.../ws or wss://.../ws)");
                    ui.text_edit_singleline(&mut self.proxy_addr);
                    ui.label("Proxy Password");
                    ui.add(egui::TextEdit::singleline(&mut self.proxy_password).password(true));
                    ui.label("Server Name");
                    ui.text_edit_singleline(&mut self.server_name);
                    ui.label("Server Password");
                    ui.add(egui::TextEdit::singleline(&mut self.server_password).password(true));
                    ui.checkbox(&mut self.use_p2p, "Use P2P through TURN if possible");

                    if ui.button("Connect").clicked() {
                        let cfg = ConnectConfig {
                            proxy_addr: self.proxy_addr.clone(),
                            proxy_password: self.proxy_password.clone(),
                            server_name: self.server_name.clone(),
                            server_password: self.server_password.clone(),
                            use_p2p: self.use_p2p,
                        };
                        self.start_connection(cfg);
                        self.show_connect_dialog = false;
                    }
                });
            self.show_connect_dialog = open;
        }

        if self.show_terminal_window {
            let mut open = self.show_terminal_window;
            egui::Window::new("Remote Terminal")
                .open(&mut open)
                .default_size([800.0, 500.0])
                .show(ctx, |ui| {
                    ui.label("Output");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.logs)
                            .desired_rows(20)
                            .interactive(false),
                    );
                    ui.separator();
                    ui.horizontal(|ui| {
                        let input_width = (ui.available_width() - 170.0).clamp(140.0, 700.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.command_input)
                                .desired_width(input_width)
                                .hint_text("Enter command"),
                        );

                        if ui.button("Send").clicked() {
                            let cmd = self.command_input.trim().to_string();
                            if !cmd.is_empty() {
                                self.send_command(cmd);
                                self.command_input.clear();
                            }
                        }

                        if ui.button("Disconnect").clicked() {
                            self.disconnect();
                        }
                    });
                });
            self.show_terminal_window = open;
        }
    }
}

impl ClientApp {
    fn start_connection(&mut self, cfg: ConnectConfig) {
        let (event_tx, event_rx) = mpsc::channel::<NetEvent>();
        let (command_tx, command_rx) = tokio_mpsc::unbounded_channel::<NetCommand>();

        self.logs.clear();
        self.status = "Connecting...".to_string();
        self.transport = "Pending".to_string();
        self.event_rx = Some(event_rx);
        self.command_tx = Some(command_tx);

        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new();
            let Ok(runtime) = runtime else {
                let _ = event_tx.send(NetEvent::Error("failed to start tokio runtime".to_string()));
                return;
            };

            runtime.block_on(async move {
                if let Err(err) = network_task(cfg, command_rx, event_tx.clone()).await {
                    let _ = event_tx.send(NetEvent::Error(err.to_string()));
                }
            });
        });
    }

    fn poll_events(&mut self) {
        let mut keep_receiving = true;
        while keep_receiving {
            let next_event = self.event_rx.as_ref().and_then(|rx| rx.try_recv().ok());

            match next_event {
                Some(NetEvent::Status(msg)) => {
                    self.status = msg;
                }
                Some(NetEvent::Transport(msg)) => {
                    self.transport = msg;
                }
                Some(NetEvent::Servers(servers)) => {
                    self.known_servers.clear();
                    self.known_servers.push("<manual>".to_string());
                    for server in servers {
                        self.known_servers.push(server);
                    }
                }
                Some(NetEvent::CommandSent { transport, command }) => {
                    self.logs
                        .push_str(&format!("[sent via {}] {}\n", transport, command));
                }
                Some(NetEvent::Connected {
                    session_id,
                    server_name,
                    via_p2p,
                    turn,
                }) => {
                    self.session_id = Some(session_id);
                    self.status = format!("Connected to {}", server_name);
                    self.show_terminal_window = true;
                    self.logs
                        .push_str(&format!("Connected. Session: {}\n", session_id));
                    if via_p2p {
                        self.logs.push_str(
                            "P2P requested. Attempting TURN peer connection first; fallback is WebSocket relay.\n",
                        );
                    } else {
                        self.logs.push_str("Using WebSocket relay transport.\n");
                        self.transport = "WebSocket relay".to_string();
                    }
                    if let Some(turn) = turn {
                        self.logs.push_str(&format!(
                            "TURN provided: {} (user: {})\n",
                            turn.url, turn.username
                        ));
                    }
                }
                Some(NetEvent::Output(chunk)) => {
                    self.logs.push_str(&chunk);
                    if !chunk.ends_with('\n') {
                        self.logs.push('\n');
                    }
                }
                Some(NetEvent::SessionClosed(reason)) => {
                    self.status = "Disconnected".to_string();
                    self.transport = "None".to_string();
                    self.logs.push_str(&format!("Session closed: {}\n", reason));
                    self.session_id = None;
                    self.command_tx = None;
                }
                Some(NetEvent::Error(reason)) => {
                    self.status = format!("Error: {}", reason);
                    self.transport = "None".to_string();
                    self.logs.push_str(&format!("Error: {}\n", reason));
                    self.session_id = None;
                    self.command_tx = None;
                }
                None => {
                    keep_receiving = false;
                }
            }
        }
    }

    fn send_command(&mut self, cmd: String) {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(NetCommand::SendCommand(cmd));
        }
    }

    fn disconnect(&mut self) {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(NetCommand::Disconnect);
        }
    }
}

async fn network_task(
    cfg: ConnectConfig,
    mut command_rx: tokio_mpsc::UnboundedReceiver<NetCommand>,
    event_tx: mpsc::Sender<NetEvent>,
) -> anyhow::Result<()> {
    let (ws_stream, _) = connect_async(&cfg.proxy_addr).await?;
    let (mut write, mut read) = ws_stream.split();

    let (ws_send_tx, mut ws_send_rx) = tokio_mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(text) = ws_send_rx.recv().await {
            if write.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    send_json(
        &ws_send_tx,
        &ClientToProxy::AuthProxy {
            proxy_password: cfg.proxy_password,
            role: AuthRole::Client,
        },
    )?;

    send_json(&ws_send_tx, &ClientToProxy::ListServers)?;

    send_json(
        &ws_send_tx,
        &ClientToProxy::ConnectServer {
            server_name: cfg.server_name,
            server_password: cfg.server_password,
            use_p2p: cfg.use_p2p,
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
                    let _ = event_tx.send(NetEvent::SessionClosed("proxy socket closed".to_string()));
                    break;
                };

                let message = message?;
                let Message::Text(text) = message else { continue; };

                let Ok(parsed) = serde_json::from_str::<ProxyToPeer>(&text) else {
                    continue;
                };

                match parsed {
                    ProxyToPeer::AuthOk { .. } => {
                        let _ = event_tx.send(NetEvent::Status("Authenticated to proxy".to_string()));
                    }
                    ProxyToPeer::AuthError { reason } | ProxyToPeer::ConnectionError { reason } => {
                        let _ = event_tx.send(NetEvent::Error(reason));
                        break;
                    }
                    ProxyToPeer::ServersList { servers } => {
                        let _ = event_tx.send(NetEvent::Servers(servers.clone()));
                        let _ = event_tx.send(NetEvent::Status(format!("{} server(s) available", servers.len())));
                    }
                    ProxyToPeer::Connected { session_id, server_name, via_p2p, turn } => {
                        active_session = Some(session_id);
                        let _ = event_tx.send(NetEvent::Connected { session_id, server_name, via_p2p, turn: turn.clone() });

                        if via_p2p {
                            if let Some(turn_cfg) = turn {
                                let _ = event_tx.send(NetEvent::Transport("Attempting P2P via TURN".to_string()));
                                let (pc, dc) = create_client_peer_connection(
                                    session_id,
                                    turn_cfg,
                                    ws_send_tx.clone(),
                                    event_tx.clone(),
                                    p2p_ready.clone(),
                                ).await?;
                                *data_channel.lock().await = Some(dc);
                                let offer = pc.create_offer(None).await?;
                                pc.set_local_description(offer).await?;
                                if let Some(local) = pc.local_description().await {
                                    send_json(&ws_send_tx, &ClientToProxy::ClientSignal {
                                        session_id,
                                        signal: SignalPayload::SdpOffer { sdp: local.sdp },
                                    })?;
                                }
                                peer_connection = Some(pc);
                            } else {
                                let _ = event_tx.send(NetEvent::Transport("WebSocket relay (no TURN credentials)".to_string()));
                            }
                        } else {
                            let _ = event_tx.send(NetEvent::Transport("WebSocket relay".to_string()));
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
                                    let init = RTCIceCandidateInit {
                                        candidate,
                                        sdp_mid,
                                        sdp_mline_index,
                                        username_fragment: None,
                                    };
                                    pc.add_ice_candidate(init).await?;
                                }
                                SignalPayload::SdpOffer { .. } => {}
                            }
                        }
                    }
                    ProxyToPeer::Output { session_id, output, .. } => {
                        if Some(session_id) == active_session {
                            let _ = event_tx.send(NetEvent::Output(output));
                        }
                    }
                    ProxyToPeer::SessionClosed { session_id, reason } => {
                        if Some(session_id) == active_session {
                            if let Some(pc) = &peer_connection {
                                let _ = pc.close().await;
                            }
                            let _ = event_tx.send(NetEvent::SessionClosed(reason));
                            break;
                        }
                    }
                }
            }
            command = command_rx.recv() => {
                let Some(command) = command else { break; };

                match command {
                    NetCommand::SendCommand(command_text) => {
                        if let Some(session_id) = active_session {
                            if p2p_ready.load(Ordering::SeqCst) {
                                let dc = data_channel.lock().await.clone();
                                if let Some(dc) = dc {
                                    let _ = event_tx.send(NetEvent::Transport("P2P data channel".to_string()));
                                    let send_text = command_text.clone();
                                    let _ = dc.send_text(send_text).await;
                                    let _ = event_tx.send(NetEvent::CommandSent {
                                        transport: "P2P data channel".to_string(),
                                        command: command_text,
                                    });
                                    continue;
                                }
                            }

                            let _ = event_tx.send(NetEvent::Transport("WebSocket relay".to_string()));
                            let sent_command = command_text.clone();
                            send_json(&ws_send_tx, &ClientToProxy::ClientCommand {
                                session_id,
                                command: command_text,
                            })?;
                            let _ = event_tx.send(NetEvent::CommandSent {
                                transport: "WebSocket relay".to_string(),
                                command: sent_command,
                            });
                        }
                    }
                    NetCommand::Disconnect => {
                        if let Some(session_id) = active_session {
                            let _ = send_json(&ws_send_tx, &ClientToProxy::DisconnectSession { session_id });
                        }
                        if let Some(pc) = &peer_connection {
                            let _ = pc.close().await;
                        }
                        let _ = event_tx.send(NetEvent::SessionClosed("client requested disconnect".to_string()));
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

fn send_json(tx: &tokio_mpsc::UnboundedSender<String>, payload: &impl Serialize) -> anyhow::Result<()> {
    let text = serde_json::to_string(payload)?;
    let _ = tx.send(text);
    Ok(())
}

async fn create_client_peer_connection(
    session_id: Uuid,
    turn: TurnCredentials,
    ws_tx: tokio_mpsc::UnboundedSender<String>,
    event_tx: mpsc::Sender<NetEvent>,
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
                    let _ = send_json(
                        &ws_tx_inner,
                        &ClientToProxy::ClientSignal {
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

    let dc = pc
        .create_data_channel(
            "cmd",
            Some(RTCDataChannelInit {
                ordered: Some(true),
                ..Default::default()
            }),
        )
        .await?;

    let ready_flag = p2p_ready.clone();
    let event_tx_open = event_tx.clone();
    dc.on_open(Box::new(move || {
        let ready_flag = ready_flag.clone();
        let event_tx_open = event_tx_open.clone();
        Box::pin(async move {
            ready_flag.store(true, Ordering::SeqCst);
            let _ = event_tx_open.send(NetEvent::Transport("P2P data channel".to_string()));
            let _ = event_tx_open.send(NetEvent::Status("P2P channel established".to_string()));
        })
    }));

    let ready_flag_close = p2p_ready.clone();
    let event_tx_close = event_tx.clone();
    dc.on_close(Box::new(move || {
        let ready_flag_close = ready_flag_close.clone();
        let event_tx_close = event_tx_close.clone();
        Box::pin(async move {
            ready_flag_close.store(false, Ordering::SeqCst);
            let _ = event_tx_close.send(NetEvent::Transport("WebSocket relay".to_string()));
            let _ = event_tx_close.send(NetEvent::Status("P2P channel closed; using WebSocket relay".to_string()));
        })
    }));

    dc.on_message(Box::new(move |msg| {
        let event_tx_msg = event_tx.clone();
        Box::pin(async move {
            let text = String::from_utf8_lossy(&msg.data).to_string();
            let _ = event_tx_msg.send(NetEvent::Output(text));
        })
    }));

    Ok((pc, dc))
}

fn main() {
    let _runmat_installed_marker = "runmat-runtime";

    let options = eframe::NativeOptions::default();
    if let Err(err) = eframe::run_native(
        "RS Peer Workspace Client",
        options,
        Box::new(|_cc| Ok(Box::<ClientApp>::default())),
    ) {
        eprintln!("failed to launch egui client: {err}");
    }
}
