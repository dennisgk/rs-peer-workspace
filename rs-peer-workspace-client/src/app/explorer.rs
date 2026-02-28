use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;

use eframe::egui;
use rs_peer_workspace_shared::app::{RpcAction, RpcRequest};
use rs_peer_workspace_shared::project::{
    display_name_for_path, is_text_file, EditorSource, EditorTab, FolderSource, ProjectFolder,
};
use uuid::Uuid;

use super::state::WorkspaceApp;
use super::tree::list_local_directory;
use super::types::{PendingAction, RemoteFolderPicker, TreeEntry};

impl WorkspaceApp {
    pub fn draw_explorer(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("explorer")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.heading("Explorer");
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.collapsing("Local", |ui| {
                        let locals: Vec<_> = self
                            .project
                            .folders
                            .iter()
                            .filter(|folder| matches!(folder.source, FolderSource::Local { .. }))
                            .cloned()
                            .collect();
                        for folder in locals {
                            self.render_folder_root(ui, &folder);
                        }
                    });

                    let mut groups: BTreeMap<String, Vec<ProjectFolder>> = BTreeMap::new();
                    for folder in &self.project.folders {
                        if let FolderSource::Remote { connection_name, .. } = &folder.source {
                            groups
                                .entry(connection_name.clone())
                                .or_default()
                                .push(folder.clone());
                        }
                    }

                    for (connection_name, folders) in groups {
                        ui.collapsing(connection_name, |ui| {
                            for folder in folders {
                                self.render_folder_root(ui, &folder);
                            }
                        });
                    }
                });
            });
    }

    pub fn render_folder_root(&mut self, ui: &mut egui::Ui, folder: &ProjectFolder) {
        let root_path = match &folder.source {
            FolderSource::Local { path } | FolderSource::Remote { path, .. } => path.clone(),
        };
        let id = format!("folder:{root_path}");
        let is_open = self.explorer_expanded.contains(&id);

        ui.horizontal(|ui| {
            if ui.small_button(if is_open { "v" } else { ">" }).clicked() {
                if is_open {
                    self.explorer_expanded.remove(&id);
                } else {
                    self.explorer_expanded.insert(id.clone());
                    self.load_children(folder, &root_path);
                }
            }
            if ui.selectable_label(false, &folder.name).clicked() {
                self.explorer_expanded.insert(id.clone());
                self.load_children(folder, &root_path);
            }
        });

        if self.explorer_expanded.contains(&id) {
            let children = self
                .explorer_cache
                .get(&root_path)
                .cloned()
                .unwrap_or_default();
            for child in children {
                self.render_tree_entry(ui, folder, &child, 1);
            }
        }
    }

    pub fn render_tree_entry(
        &mut self,
        ui: &mut egui::Ui,
        folder: &ProjectFolder,
        entry: &TreeEntry,
        depth: usize,
    ) {
        ui.horizontal(|ui| {
            ui.add_space((depth as f32) * 16.0);
            if entry.is_dir {
                let id = format!("dir:{}", entry.path);
                let is_open = self.explorer_expanded.contains(&id);
                if ui.small_button(if is_open { "v" } else { ">" }).clicked() {
                    if is_open {
                        self.explorer_expanded.remove(&id);
                    } else {
                        self.explorer_expanded.insert(id.clone());
                        self.load_children(folder, &entry.path);
                    }
                }
                if ui.selectable_label(false, &entry.name).clicked() {
                    self.explorer_expanded.insert(id.clone());
                    self.load_children(folder, &entry.path);
                }
            } else {
                ui.label(" ");
                if ui.selectable_label(false, &entry.name).clicked() {
                    self.open_path(folder, &entry.path);
                }
            }
        });

        let id = format!("dir:{}", entry.path);
        if entry.is_dir && self.explorer_expanded.contains(&id) {
            let children = self
                .explorer_cache
                .get(&entry.path)
                .cloned()
                .unwrap_or_default();
            for child in children {
                self.render_tree_entry(ui, folder, &child, depth + 1);
            }
        }
    }

    pub fn render_picker_node(&mut self, ui: &mut egui::Ui, path: &str, depth: usize) {
        let label = display_name_for_path(path);
        let id = format!("picker:{path}");
        let is_open = self.remote_picker.expanded.contains(&id);

        ui.horizontal(|ui| {
            ui.add_space((depth as f32) * 16.0);
            if ui.small_button(if is_open { "v" } else { ">" }).clicked() {
                if is_open {
                    self.remote_picker.expanded.remove(&id);
                } else {
                    self.remote_picker.expanded.insert(id.clone());
                    self.request_picker_children(path);
                }
            }
            if ui
                .selectable_label(self.remote_picker.selected_path == path, label)
                .clicked()
            {
                self.remote_picker.selected_path = path.to_string();
            }
        });

        if is_open {
            let children = self
                .remote_picker
                .cache
                .get(path)
                .cloned()
                .unwrap_or_default();
            for child in children.into_iter().filter(|entry| entry.is_dir) {
                self.render_picker_node(ui, &child.path, depth + 1);
            }
        }
    }

    pub fn load_children(&mut self, folder: &ProjectFolder, path: &str) {
        if self.explorer_cache.contains_key(path) {
            return;
        }

        match &folder.source {
            FolderSource::Local { .. } => {
                let entries = list_local_directory(path).unwrap_or_default();
                self.explorer_cache.insert(path.to_string(), entries);
            }
            FolderSource::Remote { connection_name, .. } => {
                let request_id = Uuid::new_v4();
                self.pending.insert(
                    request_id,
                    PendingAction::LoadRemoteDirectory {
                        path: path.to_string(),
                    },
                );
                self.send_rpc(
                    connection_name,
                    RpcRequest {
                        request_id,
                        action: RpcAction::ListDirectory {
                            path: path.to_string(),
                        },
                    },
                );
            }
        }
    }

    pub fn open_path(&mut self, folder: &ProjectFolder, path: &str) {
        if !is_text_file(path) {
            self.output_lines
                .push(format!("Skipping non-text file {path}"));
            return;
        }
        if let Some(existing) = self.open_files.iter().position(|tab| tab.path == path) {
            self.selected_editor = Some(existing);
            return;
        }

        match &folder.source {
            FolderSource::Local { .. } => match fs::read_to_string(path) {
                Ok(content) => {
                    self.open_files.push(EditorTab {
                        title: display_name_for_path(path),
                        path: path.to_string(),
                        source: EditorSource::Local,
                        content,
                        dirty: false,
                    });
                    self.selected_editor = Some(self.open_files.len() - 1);
                }
                Err(err) => self
                    .output_lines
                    .push(format!("Failed to read {path}: {err}")),
            },
            FolderSource::Remote { connection_name, .. } => {
                let request_id = Uuid::new_v4();
                self.pending.insert(
                    request_id,
                    PendingAction::OpenRemoteFile {
                        path: path.to_string(),
                        title: display_name_for_path(path),
                        connection_name: connection_name.clone(),
                    },
                );
                self.send_rpc(
                    connection_name,
                    RpcRequest {
                        request_id,
                        action: RpcAction::ReadFile {
                            path: path.to_string(),
                        },
                    },
                );
            }
        }
    }

    pub fn open_remote_picker(&mut self) {
        if self.folder_form.remote_connection_name.is_empty() {
            self.output_lines
                .push("Select a connection before browsing remote folders.".to_string());
            return;
        }

        self.remote_picker = RemoteFolderPicker {
            open: true,
            connection_name: self.folder_form.remote_connection_name.clone(),
            selected_path: String::new(),
            roots: Vec::new(),
            cache: HashMap::new(),
            expanded: HashSet::new(),
        };

        let request_id = Uuid::new_v4();
        self.pending
            .insert(request_id, PendingAction::LoadPickerRoots);
        let connection_name = self.remote_picker.connection_name.clone();
        self.send_rpc(
            &connection_name,
            RpcRequest {
                request_id,
                action: RpcAction::ListRoots,
            },
        );
    }

    pub fn request_picker_children(&mut self, path: &str) {
        if self.remote_picker.cache.contains_key(path) {
            return;
        }

        let request_id = Uuid::new_v4();
        self.pending.insert(
            request_id,
            PendingAction::LoadPickerDirectory {
                path: path.to_string(),
            },
        );
        let connection_name = self.remote_picker.connection_name.clone();
        self.send_rpc(
            &connection_name,
            RpcRequest {
                request_id,
                action: RpcAction::ListDirectory {
                    path: path.to_string(),
                },
            },
        );
    }
}
