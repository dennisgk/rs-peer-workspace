use eframe::egui;
use rfd::FileDialog;

use super::state::WorkspaceApp;

impl WorkspaceApp {
    pub fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.modifiers.ctrl && input.key_pressed(egui::Key::S)) {
            self.save_active_editor();
        }
    }

    pub fn draw_menu(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Create Project").clicked() {
                        self.reset_project();
                        ui.close_menu();
                    }
                    if ui.button("Open Project").clicked() {
                        if let Some(path) = FileDialog::new()
                            .add_filter("RS Peer Workspace", &["rpw"])
                            .pick_file()
                        {
                            self.open_project(path);
                        }
                        ui.close_menu();
                    }
                    if ui.button("Save Project").clicked() {
                        self.save_project();
                        ui.close_menu();
                    }
                });

                ui.menu_button("Edit", |ui| {
                    if ui.button("Save").clicked() {
                        self.save_active_editor();
                        ui.close_menu();
                    }
                    if ui.button("Add Connection").clicked() {
                        self.show_add_connection = true;
                        ui.close_menu();
                    }
                    if ui.button("Add Folder").clicked() {
                        self.show_add_folder = true;
                        ui.close_menu();
                    }
                });

                ui.menu_button("Terminal", |ui| {
                    if ui.button("New Terminal").clicked() {
                        self.show_new_terminal = true;
                        ui.close_menu();
                    }
                });
            });
        });
    }

    pub fn draw_add_connection(&mut self, ctx: &egui::Context) {
        if !self.show_add_connection {
            return;
        }

        let mut open = self.show_add_connection;
        egui::Window::new("Add Connection")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Custom Name");
                ui.text_edit_singleline(&mut self.connection_form.name);
                ui.label("Proxy Address");
                ui.text_edit_singleline(&mut self.connection_form.proxy_addr);
                ui.label("Proxy Password");
                ui.add(
                    egui::TextEdit::singleline(&mut self.connection_form.proxy_password)
                        .password(true),
                );
                ui.label("Server Name");
                ui.text_edit_singleline(&mut self.connection_form.server_name);
                ui.label("Server Password");
                ui.add(
                    egui::TextEdit::singleline(&mut self.connection_form.server_password)
                        .password(true),
                );
                ui.checkbox(&mut self.connection_form.prefer_p2p, "Try P2P first");
                if ui.button("Add").clicked() {
                    self.add_connection();
                    self.show_add_connection = false;
                }
            });
        self.show_add_connection = open;
    }

    pub fn draw_add_folder(&mut self, ctx: &egui::Context) {
        if !self.show_add_folder {
            return;
        }

        let mut open = self.show_add_folder;
        egui::Window::new("Add Folder")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Display Name");
                ui.text_edit_singleline(&mut self.folder_form.name);
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.folder_form.is_remote, false, "Local");
                    ui.radio_value(&mut self.folder_form.is_remote, true, "Remote");
                });

                if self.folder_form.is_remote {
                    egui::ComboBox::from_id_salt("remote-connection")
                        .selected_text(if self.folder_form.remote_connection_name.is_empty() {
                            "Select connection"
                        } else {
                            &self.folder_form.remote_connection_name
                        })
                        .show_ui(ui, |ui| {
                            for connection in &self.project.connections {
                                if ui
                                    .selectable_label(
                                        self.folder_form.remote_connection_name == connection.name,
                                        &connection.name,
                                    )
                                    .clicked()
                                {
                                    self.folder_form.remote_connection_name =
                                        connection.name.clone();
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.folder_form.remote_path);
                        if ui.button("Browse").clicked() {
                            self.open_remote_picker();
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.folder_form.local_path);
                        if ui.button("Browse").clicked() {
                            if let Some(path) = FileDialog::new().pick_folder() {
                                self.folder_form.local_path = path.to_string_lossy().to_string();
                            }
                        }
                    });
                }

                if ui.button("Add Folder").clicked() {
                    self.add_folder();
                    self.show_add_folder = false;
                }
            });
        self.show_add_folder = open;
    }

    pub fn draw_new_terminal(&mut self, ctx: &egui::Context) {
        if !self.show_new_terminal {
            return;
        }

        let mut open = self.show_new_terminal;
        egui::Window::new("New Terminal")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                egui::ComboBox::from_id_salt("terminal-connection")
                    .selected_text(if self.terminal_form.connection_name.is_empty() {
                        "Select connection"
                    } else {
                        &self.terminal_form.connection_name
                    })
                    .show_ui(ui, |ui| {
                        for connection in &self.project.connections {
                            if ui
                                .selectable_label(
                                    self.terminal_form.connection_name == connection.name,
                                    &connection.name,
                                )
                                .clicked()
                            {
                                self.terminal_form.connection_name = connection.name.clone();
                            }
                        }
                    });
                if ui.button("Open Terminal").clicked() {
                    self.create_terminal();
                    self.show_new_terminal = false;
                }
            });
        self.show_new_terminal = open;
    }

    pub fn draw_remote_picker(&mut self, ctx: &egui::Context) {
        if !self.remote_picker.open {
            return;
        }

        let mut open = self.remote_picker.open;
        egui::Window::new("Remote Folder Picker")
            .open(&mut open)
            .default_size([520.0, 420.0])
            .show(ctx, |ui| {
                ui.label(format!("Connection: {}", self.remote_picker.connection_name));
                let roots = self.remote_picker.roots.clone();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for root in roots {
                        self.render_picker_node(ui, &root, 0);
                    }
                });
                ui.separator();
                ui.label(format!("Selected: {}", self.remote_picker.selected_path));
                if ui.button("Use Folder").clicked() {
                    self.folder_form.remote_path = self.remote_picker.selected_path.clone();
                    self.remote_picker.open = false;
                }
            });
        self.remote_picker.open = open;
    }
}
