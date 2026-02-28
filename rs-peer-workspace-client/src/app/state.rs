use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use rs_peer_workspace_shared::project::{
    default_connection_form_addr, EditorTab, ProjectFile, TerminalTab,
};
use uuid::Uuid;

use crate::net::ConnectionEvent;

use super::types::{
    BottomTab, ConnectionForm, ConnectionState, FolderForm, PendingAction, RemoteFolderPicker,
    TerminalForm, TreeEntry,
};

pub struct WorkspaceApp {
    pub project: ProjectFile,
    pub project_path: Option<PathBuf>,
    pub event_rx: Receiver<ConnectionEvent>,
    pub event_tx: Sender<ConnectionEvent>,
    pub connections: HashMap<String, ConnectionState>,
    pub pending: HashMap<Uuid, PendingAction>,
    pub show_add_connection: bool,
    pub show_add_folder: bool,
    pub show_new_terminal: bool,
    pub connection_form: ConnectionForm,
    pub folder_form: FolderForm,
    pub terminal_form: TerminalForm,
    pub remote_picker: RemoteFolderPicker,
    pub output_lines: Vec<String>,
    pub task_lines: Vec<String>,
    pub explorer_cache: HashMap<String, Vec<TreeEntry>>,
    pub explorer_expanded: HashSet<String>,
    pub open_files: Vec<EditorTab>,
    pub selected_editor: Option<usize>,
    pub terminals: Vec<TerminalTab>,
    pub selected_terminal: Option<usize>,
    pub active_bottom_tab: BottomTab,
}

impl Default for WorkspaceApp {
    fn default() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            project: ProjectFile::default(),
            project_path: None,
            event_rx,
            event_tx,
            connections: HashMap::new(),
            pending: HashMap::new(),
            show_add_connection: false,
            show_add_folder: false,
            show_new_terminal: false,
            connection_form: ConnectionForm {
                proxy_addr: default_connection_form_addr(),
                prefer_p2p: true,
                ..Default::default()
            },
            folder_form: FolderForm::default(),
            terminal_form: TerminalForm::default(),
            remote_picker: RemoteFolderPicker::default(),
            output_lines: vec!["Ready.".to_string()],
            task_lines: Vec::new(),
            explorer_cache: HashMap::new(),
            explorer_expanded: HashSet::new(),
            open_files: Vec::new(),
            selected_editor: None,
            terminals: Vec::new(),
            selected_terminal: None,
            active_bottom_tab: BottomTab::Output,
        }
    }
}

impl eframe::App for WorkspaceApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_events();
        self.handle_shortcuts(ctx);
        self.draw_menu(ctx);
        self.draw_add_connection(ctx);
        self.draw_add_folder(ctx);
        self.draw_new_terminal(ctx);
        self.draw_remote_picker(ctx);
        self.draw_explorer(ctx);
        self.draw_bottom(ctx);
        self.draw_editor(ctx);
    }
}
