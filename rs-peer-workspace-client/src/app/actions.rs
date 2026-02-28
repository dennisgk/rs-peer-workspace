use std::fs;
use std::path::PathBuf;

use rs_peer_workspace_shared::app::{RpcAction, RpcRequest, RpcResponse, RpcResult};
use rs_peer_workspace_shared::project::{
    default_connection_form_addr, display_name_for_path, EditorSource, FolderSource,
    ProjectConnection, ProjectFile, ProjectFolder, TerminalTab,
};
use uuid::Uuid;

use crate::net::{spawn_connection, ConnectionCommand, ConnectionEvent};

use super::state::WorkspaceApp;
use super::tree::tree_from_entry;
use super::types::{BottomTab, ConnectionForm, ConnectionState, FolderForm, PendingAction, TerminalForm};

impl WorkspaceApp {
    pub fn reset_project(&mut self) {
        self.disconnect_all();
        self.project = ProjectFile::default();
        self.project_path = None;
        self.pending.clear();
        self.explorer_cache.clear();
        self.explorer_expanded.clear();
        self.open_files.clear();
        self.selected_editor = None;
        self.terminals.clear();
        self.selected_terminal = None;
        self.connections.clear();
        self.output_lines.push("Created new project.".to_string());
    }

    fn disconnect_all(&mut self) {
        for state in self.connections.values() {
            let _ = state.command_tx.send(ConnectionCommand::Disconnect);
        }
    }

    pub fn add_connection(&mut self) {
        let name = self.connection_form.name.trim();
        if name.is_empty() {
            self.output_lines.push("Connection name is required.".to_string());
            return;
        }
        if self.connection_form.proxy_addr.trim().is_empty() {
            self.output_lines.push("Proxy address is required.".to_string());
            return;
        }

        let connection = ProjectConnection {
            name: name.to_string(),
            proxy_addr: self.connection_form.proxy_addr.trim().to_string(),
            proxy_password: self.connection_form.proxy_password.clone(),
            server_name: self.connection_form.server_name.trim().to_string(),
            server_password: self.connection_form.server_password.clone(),
            prefer_p2p: self.connection_form.prefer_p2p,
        };

        self.project.connections.retain(|item| item.name != connection.name);
        self.project.connections.push(connection.clone());

        let command_tx = spawn_connection(connection.clone(), self.event_tx.clone());
        self.connections.insert(
            connection.name.clone(),
            ConnectionState {
                command_tx,
                connected: false,
                transport: "Connecting".to_string(),
            },
        );
        self.task_lines.push(format!("[{}] connecting...", connection.name));
        self.connection_form = ConnectionForm {
            proxy_addr: default_connection_form_addr(),
            prefer_p2p: true,
            ..Default::default()
        };
    }

    pub fn add_folder(&mut self) {
        let folder = if self.folder_form.is_remote {
            if self.folder_form.remote_connection_name.trim().is_empty()
                || self.folder_form.remote_path.trim().is_empty()
            {
                self.output_lines
                    .push("Remote folder requires connection and path.".to_string());
                return;
            }
            ProjectFolder {
                name: if self.folder_form.name.trim().is_empty() {
                    display_name_for_path(&self.folder_form.remote_path)
                } else {
                    self.folder_form.name.trim().to_string()
                },
                source: FolderSource::Remote {
                    connection_name: self.folder_form.remote_connection_name.clone(),
                    path: self.folder_form.remote_path.clone(),
                },
            }
        } else {
            if self.folder_form.local_path.trim().is_empty() {
                self.output_lines.push("Local folder path is required.".to_string());
                return;
            }
            ProjectFolder {
                name: if self.folder_form.name.trim().is_empty() {
                    display_name_for_path(&self.folder_form.local_path)
                } else {
                    self.folder_form.name.trim().to_string()
                },
                source: FolderSource::Local {
                    path: self.folder_form.local_path.clone(),
                },
            }
        };

        self.project.folders.push(folder);
        self.folder_form = FolderForm::default();
        self.explorer_cache.clear();
        self.explorer_expanded.clear();
    }

    pub fn create_terminal(&mut self) {
        if self.terminal_form.connection_name.is_empty() {
            self.output_lines
                .push("Select a connection for the terminal.".to_string());
            return;
        }

        self.terminals.push(TerminalTab {
            id: Uuid::new_v4(),
            connection_name: self.terminal_form.connection_name.clone(),
            title: format!("Terminal {}", self.terminals.len() + 1),
            input: String::new(),
            output: String::new(),
        });
        self.selected_terminal = Some(self.terminals.len() - 1);
        self.active_bottom_tab = BottomTab::Terminal;
        self.terminal_form = TerminalForm::default();
    }

    pub fn run_terminal(&mut self, terminal_index: usize, command: String) {
        let Some(terminal) = self.terminals.get_mut(terminal_index) else {
            return;
        };
        terminal.output.push_str(&format!("> {command}\n"));
        let connection_name = terminal.connection_name.clone();
        let terminal_id = terminal.id;
        let request_id = Uuid::new_v4();
        self.pending.insert(
            request_id,
            PendingAction::RunTerminal {
                terminal_id,
            },
        );
        self.send_rpc(
            &connection_name,
            RpcRequest {
                request_id,
                action: RpcAction::RunCommand { command },
            },
        );
        self.active_bottom_tab = BottomTab::Tasks;
    }

    pub fn open_project(&mut self, path: PathBuf) {
        let loaded = fs::read_to_string(&path)
            .ok()
            .and_then(|text| ron::from_str::<ProjectFile>(&text).ok());

        match loaded {
            Some(project) => {
                self.reset_project();
                self.project = project;
                self.project_path = Some(path.clone());
                for connection in self.project.connections.clone() {
                    let command_tx = spawn_connection(connection.clone(), self.event_tx.clone());
                    self.connections.insert(
                        connection.name.clone(),
                        ConnectionState {
                            command_tx,
                            connected: false,
                            transport: "Connecting".to_string(),
                        },
                    );
                }
                self.output_lines
                    .push(format!("Opened project {}", path.display()));
            }
            None => {
                self.output_lines
                    .push(format!("Failed to open project {}", path.display()));
            }
        }
    }

    pub fn save_project(&mut self) {
        if self.project_path.is_none() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("RS Peer Workspace", &["rpw"])
                .set_file_name("workspace.rpw")
                .save_file()
            {
                self.project_path = Some(path);
            } else {
                return;
            }
        }

        if let Some(path) = &self.project_path {
            match ron::ser::to_string_pretty(&self.project, ron::ser::PrettyConfig::default()) {
                Ok(content) => match fs::write(path, content) {
                    Ok(()) => self
                        .output_lines
                        .push(format!("Saved project {}", path.display())),
                    Err(err) => self
                        .output_lines
                        .push(format!("Failed to save project: {err}")),
                },
                Err(err) => self
                    .output_lines
                    .push(format!("Failed to serialize project: {err}")),
            }
        }
    }

    pub fn save_active_editor(&mut self) {
        let Some(idx) = self.selected_editor else {
            return;
        };
        let Some(tab) = self.open_files.get(idx).cloned() else {
            return;
        };

        match tab.source {
            EditorSource::Local => match fs::write(&tab.path, &tab.content) {
                Ok(()) => {
                    if let Some(open_tab) = self.open_files.get_mut(idx) {
                        open_tab.dirty = false;
                    }
                    self.output_lines.push(format!("Saved {}", tab.path));
                }
                Err(err) => self
                    .output_lines
                    .push(format!("Failed to save {}: {err}", tab.path)),
            },
            EditorSource::Remote { connection_name } => {
                let request_id = Uuid::new_v4();
                self.pending.insert(
                    request_id,
                    PendingAction::SaveRemoteFile {
                        path: tab.path.clone(),
                    },
                );
                self.send_rpc(
                    &connection_name,
                    RpcRequest {
                        request_id,
                        action: RpcAction::WriteFile {
                            path: tab.path.clone(),
                            content: tab.content.clone(),
                        },
                    },
                );
            }
        }
    }

    pub fn send_rpc(&mut self, connection_name: &str, request: RpcRequest) {
        let Some(connection) = self.connections.get(connection_name) else {
            self.output_lines
                .push(format!("Unknown connection {connection_name}"));
            return;
        };
        let _ = connection.command_tx.send(ConnectionCommand::SendRpc(request));
    }

    pub fn poll_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                ConnectionEvent::Status {
                    connection_name,
                    message,
                } => self
                    .output_lines
                    .push(format!("[{connection_name}] {message}")),
                ConnectionEvent::Transport {
                    connection_name,
                    message,
                } => {
                    if let Some(connection) = self.connections.get_mut(&connection_name) {
                        connection.transport = message.clone();
                    }
                    self.output_lines
                        .push(format!("[{connection_name}] transport: {message}"));
                }
                ConnectionEvent::Connected { connection_name } => {
                    if let Some(connection) = self.connections.get_mut(&connection_name) {
                        connection.connected = true;
                    }
                    self.task_lines.push(format!("[{connection_name}] connected"));
                }
                ConnectionEvent::RpcResponse {
                    connection_name,
                    response,
                } => {
                    if let Some(action) = self.pending.remove(&response.request_id) {
                        self.handle_rpc_response(&connection_name, action, response);
                    }
                }
                ConnectionEvent::Error {
                    connection_name,
                    message,
                } => self
                    .output_lines
                    .push(format!("[{connection_name}] error: {message}")),
                ConnectionEvent::Closed {
                    connection_name,
                    reason,
                } => {
                    self.output_lines
                        .push(format!("[{connection_name}] closed: {reason}"));
                    if let Some(connection) = self.connections.get_mut(&connection_name) {
                        connection.connected = false;
                        connection.transport = "Disconnected".to_string();
                    }
                }
            }
        }
    }

    pub fn handle_rpc_response(
        &mut self,
        connection_name: &str,
        action: PendingAction,
        response: RpcResponse,
    ) {
        match (action, response.result) {
            (
                PendingAction::OpenRemoteFile {
                    path,
                    title,
                    connection_name,
                },
                RpcResult::FileContent { content, .. },
            ) => {
                self.open_files.push(rs_peer_workspace_shared::project::EditorTab {
                    title,
                    path,
                    source: EditorSource::Remote { connection_name },
                    content,
                    dirty: false,
                });
                self.selected_editor = Some(self.open_files.len() - 1);
            }
            (PendingAction::SaveRemoteFile { path }, RpcResult::WriteComplete { .. }) => {
                if let Some(tab) = self.open_files.iter_mut().find(|tab| tab.path == path) {
                    tab.dirty = false;
                }
                self.output_lines
                    .push(format!("[{connection_name}] saved {path}"));
            }
            (
                PendingAction::LoadRemoteDirectory { path },
                RpcResult::DirectoryEntries { entries, .. },
            ) => {
                self.explorer_cache
                    .insert(path, entries.into_iter().map(tree_from_entry).collect());
            }
            (PendingAction::LoadPickerRoots, RpcResult::Roots { roots }) => {
                self.remote_picker.roots = roots;
            }
            (
                PendingAction::LoadPickerDirectory { path },
                RpcResult::DirectoryEntries { entries, .. },
            ) => {
                self.remote_picker
                    .cache
                    .insert(path, entries.into_iter().map(tree_from_entry).collect());
            }
            (PendingAction::RunTerminal { terminal_id }, RpcResult::CommandOutput { output }) => {
                if let Some(term) = self.terminals.iter_mut().find(|term| term.id == terminal_id)
                {
                    term.output.push_str(&output);
                    if !output.ends_with('\n') {
                        term.output.push('\n');
                    }
                }
                self.active_bottom_tab = BottomTab::Terminal;
            }
            (_, RpcResult::Error { message }) => {
                self.output_lines
                    .push(format!("[{connection_name}] {message}"));
                self.active_bottom_tab = BottomTab::Output;
            }
            _ => {}
        }
    }
}
