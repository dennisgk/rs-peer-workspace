use clap::Parser;
use futures_util::{Sink, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

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
    },
    RunCommand {
        session_id: Uuid,
        command: String,
    },
    SessionClosed {
        session_id: Uuid,
        reason: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let _runmat_installed_marker = "runmat-runtime";

    let (ws_stream, _) = connect_async(&args.proxy_url).await?;
    println!("connected to proxy {}", args.proxy_url);

    let (mut write, mut read) = ws_stream.split();

    send_json(
        &mut write,
        &ClientToProxy::AuthProxy {
            proxy_password: args.proxy_password.clone(),
            role: AuthRole::Server,
        },
    )
    .await?;

    send_json(
        &mut write,
        &ClientToProxy::RegisterServer {
            server_name: args.server_name.clone(),
            server_password: args.server_password.clone(),
        },
    )
    .await?;

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
            } => {
                println!("client {client_id} joined session {session_id}");
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
                send_json(&mut write, &msg).await?;
            }
            ProxyToPeer::SessionClosed { session_id, reason } => {
                println!("session {session_id} closed: {reason}");
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
