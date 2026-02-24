use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use futures_util::{Sink, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnCredentials {
    url: String,
    username: String,
    password: String,
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
    Servers(Vec<String>),
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
                    ui.label("Proxy Address (ws://.../ws)");
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
            let next_event = self
                .event_rx
                .as_ref()
                .and_then(|rx| rx.try_recv().ok());

            match next_event {
                Some(NetEvent::Status(msg)) => {
                    self.status = msg;
                }
                Some(NetEvent::Servers(servers)) => {
                    self.known_servers.clear();
                    self.known_servers.push("<manual>".to_string());
                    for server in servers {
                        self.known_servers.push(server);
                    }
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
                    self.transport = "WebSocket relay".to_string();
                    self.logs.push_str(&format!(
                        "Connected. Session: {}\n",
                        session_id
                    ));
                    if via_p2p {
                        self.logs.push_str(
                            "P2P requested. TURN credentials received if available, but command traffic is currently WebSocket relay.\n",
                        );
                    } else {
                        self.logs.push_str("Using WebSocket relay transport.\n");
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
            self.logs
                .push_str(&format!("[send via {}] {}\n", self.transport, cmd));
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

    send_json(
        &mut write,
        &ClientToProxy::AuthProxy {
            proxy_password: cfg.proxy_password,
            role: AuthRole::Client,
        },
    )
    .await?;

    send_json(&mut write, &ClientToProxy::ListServers).await?;

    send_json(
        &mut write,
        &ClientToProxy::ConnectServer {
            server_name: cfg.server_name,
            server_password: cfg.server_password,
            use_p2p: cfg.use_p2p,
        },
    )
    .await?;

    let mut active_session: Option<Uuid> = None;

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
                        let _ = event_tx.send(NetEvent::Connected { session_id, server_name, via_p2p, turn });
                    }
                    ProxyToPeer::Output { session_id, output, .. } => {
                        if Some(session_id) == active_session {
                            let _ = event_tx.send(NetEvent::Output(output));
                        }
                    }
                    ProxyToPeer::SessionClosed { session_id, reason } => {
                        if Some(session_id) == active_session {
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
                            send_json(&mut write, &ClientToProxy::ClientCommand {
                                session_id,
                                command: command_text,
                            }).await?;
                        }
                    }
                    NetCommand::Disconnect => {
                        if let Some(session_id) = active_session {
                            let _ = send_json(&mut write, &ClientToProxy::DisconnectSession { session_id }).await;
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

async fn send_json(
    sink: &mut (impl Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
    payload: &impl Serialize,
) -> anyhow::Result<()> {
    let text = serde_json::to_string(payload)?;
    sink.send(Message::Text(text.into())).await?;
    Ok(())
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
