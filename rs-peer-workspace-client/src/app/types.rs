use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::net::ConnectionCommand;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BottomTab {
    Output,
    Tasks,
    Terminal,
}

#[derive(Debug, Clone)]
pub struct ConnectionState {
    pub command_tx: tokio::sync::mpsc::UnboundedSender<ConnectionCommand>,
    pub connected: bool,
    pub transport: String,
}

#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Default)]
pub struct ConnectionForm {
    pub name: String,
    pub proxy_addr: String,
    pub proxy_password: String,
    pub server_name: String,
    pub server_password: String,
    pub prefer_p2p: bool,
}

#[derive(Default)]
pub struct FolderForm {
    pub name: String,
    pub is_remote: bool,
    pub local_path: String,
    pub remote_connection_name: String,
    pub remote_path: String,
}

#[derive(Default)]
pub struct TerminalForm {
    pub connection_name: String,
}

#[derive(Default)]
pub struct RemoteFolderPicker {
    pub open: bool,
    pub connection_name: String,
    pub selected_path: String,
    pub roots: Vec<String>,
    pub cache: HashMap<String, Vec<TreeEntry>>,
    pub expanded: HashSet<String>,
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    OpenRemoteFile {
        path: String,
        title: String,
        connection_name: String,
    },
    SaveRemoteFile {
        path: String,
    },
    LoadRemoteDirectory {
        path: String,
    },
    LoadPickerRoots,
    LoadPickerDirectory {
        path: String,
    },
    RunTerminal {
        terminal_id: Uuid,
    },
}
